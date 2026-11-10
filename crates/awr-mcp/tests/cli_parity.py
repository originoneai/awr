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
import sys
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
            # Budget selection is CLI transport metadata; compare every context fact unchanged.
            self.assertEqual(cli.pop("budget"), {"applied": 5000, "basis": "default"})
            self.equal_read(cli, mcp)  # No hash, rendered text or gap normalization.
        _, error = self.cli(["context", "compile", "--work", "W", "--detached", "--budget", "1"], code=1)
        mcp_error = self.tool("awr_context_compile", {"work": "W", "detached": True, "budget": 1}, error=True, readonly=True)
        self.assertIn("--work W --detached", error["details"].pop("next_action"))
        self.assertNotIn("next_action", mcp_error["details"])
        self.assertEqual(error, mcp_error)

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

        draft = {"external_key": "E", "work_item_key": "W", "evidence_type": "check_run", "level": "implemented",
                 "summary": "Evidence summary " * 24, "locator": "report.json", "command": "Documented command " * 24,
                 "scope": [f"scope-{i}" for i in range(24)], "branch_id": None}
        (self.root / "evidence.json").write_text(json.dumps(draft))
        expected = self.revision()
        snapshot = self.snapshot()
        cli = self.cli(["evidence", "add", "--input", "evidence.json", "--expected-revision", expected])
        self.assertEqual(cli["trust"], "reported")
        self.assertEqual(cli["evidence"]["summary"], draft["summary"])
        self.assertEqual(cli["evidence"]["scope"], draft["scope"])
        self.restore(snapshot)
        args = {**draft, "expected_revision": expected, "work": draft["work_item_key"], "branch": "main"}
        del args["work_item_key"], args["branch_id"]
        mcp = self.tool("awr_evidence_record", args)
        self.assertEqual(mcp["trust"], "reported")
        for value in [cli, mcp]:
            self.assertEqual(len(value.pop("event_id")), 26)
            self.assertEqual(len(value["evidence"].pop("id")), 26)
        self.assertEqual(cli, mcp)

    def test_manual_evidence_type_strings_are_reported_in_cli_and_mcp(self):
        for kind in ["check_run", "test_run", "review"]:
            with self.subTest(evidence_type=kind):
                key = "manual-" + kind
                draft = {"external_key": key, "work_item_key": "W", "evidence_type": kind,
                         "level": "implemented", "summary": "Caller-supplied report",
                         "locator": "run.json", "scope": ["W"], "branch_id": None}
                (self.root / "evidence.json").write_text(json.dumps(draft))
                expected = self.revision()
                snapshot = self.snapshot()
                cli = self.cli(["evidence", "add", "--input", "evidence.json", "--expected-revision", expected])
                cli_evidence = self.cli(["evidence", "show", key])
                cli_work = self.cli(["work", "show", "W"])
                self.restore(snapshot)
                args = {**draft, "expected_revision": expected, "work": "W", "branch": "main"}
                del args["work_item_key"], args["branch_id"]
                mcp = self.tool("awr_evidence_record", args)
                mcp_work = self.tool("awr_work_get", {"work": "W"}, readonly=True)
                observations = {
                    "CLI registration receipt": cli["trust"],
                    "CLI registration": cli["evidence"]["trust"],
                    "CLI evidence receipt": cli_evidence["trust"],
                    "CLI evidence read": cli_evidence["evidence"]["trust"],
                    "CLI work read": next(e["trust"] for e in cli_work["evidence"] if e["external_key"] == key),
                    "MCP registration receipt": mcp["trust"],
                    "MCP registration": mcp["evidence"]["trust"],
                    "MCP work read": next(e["trust"] for e in mcp_work["evidence"] if e["external_key"] == key),
                }
                self.assertEqual(observations, dict.fromkeys(observations, "reported"))

    def test_generated_check_evidence_is_recorded_then_verified_in_cli_and_mcp(self):
        criterion = "Deliver the reviewed analysis"
        definition = {"version": 1, "purpose": "implementation", "tier": "required",
                      "examples": [{"id": "saved-analysis", "criterion": criterion, "kind": "positive",
                                    "given": "A reviewed analysis is saved", "when": "Its contents are checked",
                                    "then": "The analysis remains available",
                                    "check": {"run": [sys.executable, "-c",
                                                      "from pathlib import Path; assert Path('analysis.txt').read_text() == 'reviewed analysis\\n'"],
                                              "files": ["analysis.txt"], "timeout_seconds": 10}}]}
        work = WORK.replace("  acceptance: [Deliver the reviewed analysis]\n",
                            "  acceptance: [Deliver the reviewed analysis]\n  definition: " + json.dumps(definition) + "\n")
        (self.root / "work.yaml").write_text(work)
        (self.root / "analysis.txt").write_text("reviewed analysis\n")
        self.cli(["status"])
        sid = self.session()
        self.cli(["work", "progress", "W", "--session", sid, "--reason", "The analysis is ready for checking",
                  "--next-action", "Run the declared check", "--expected-revision", self.revision()])
        for args in [["init"], ["config", "user.email", "parity@example.invalid"],
                     ["config", "user.name", "Check parity fixture"],
                     ["add", "work.yaml", "rules.md", "goal.md", "analysis.txt", "project.toml"],
                     ["commit", "-m", "Create the check provenance fixture"]]:
            subprocess.run(["git", *args], cwd=self.root, capture_output=True, check=True)
        completion = self.root / "check-completion.json"
        checked = self.cli(["work", "check", "W", "--session", sid, "--phase", "acceptance",
                            "--write-input", completion, "--expected-revision", self.revision()])
        self.assertEqual(checked["evidence"]["trust"], "recorded")
        key = checked["evidence"]["external_key"]

        def assert_trust(expected):
            shown = self.cli(["evidence", "show", key])
            cli = self.cli(["work", "show", "W"])
            mcp = self.tool("awr_work_get", {"work": "W"}, readonly=True)
            cli_record = next(e for e in cli["evidence"] if e["external_key"] == key)
            mcp_record = next(e for e in mcp["evidence"] if e["external_key"] == key)
            self.assertEqual(shown["evidence"]["id"], checked["evidence"]["id"])
            self.assertEqual(cli_record, mcp_record)
            self.assertEqual(cli_record["locator"], checked["evidence"]["locator"])
            observations = {"CLI evidence receipt": shown["trust"], "CLI evidence": shown["evidence"]["trust"],
                            "CLI work": cli_record["trust"], "MCP work": mcp_record["trust"]}
            self.assertEqual(observations, dict.fromkeys(observations, expected))

        assert_trust("recorded")
        self.cli(["work", "complete", "W", "--session", sid, "--reason", "The report bytes match",
                  "--input", completion, "--expected-revision", self.revision()])
        assert_trust("verified")

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

    # ---- AWR-EVO-012: the five layers keep their own answers, and the entries give the same ones
    NOT_DONE = WORK.replace("id: D\n  title: Required input\n  status: completed",
                            "id: D\n  title: Required input\n  status: ready\n  next_action: Finish the input")

    def facts(self, work):
        """Change the facts behind earlier answers: the source is edited and recorded."""
        (self.root / "work.yaml").write_text(work)
        self.cli(["source", "reindex"])

    def prepared(self, failure=None):
        """work prepare through both entries. Each words a failure in its own envelope (the CLI on standard error, the
        MCP server inside the result), so the bodies are compared without it and the error code on its own."""
        if failure:
            cli, cli_error = self.cli(["work", "prepare", "W", "--budget", "9000"], code=1)
            self.assertEqual(cli_error["code"], failure)
        else:
            cli = self.cli(["work", "prepare", "W", "--budget", "9000"])
        mcp = self.tool("awr_work_prepare", {"work": "W", "budget": 9000}, error=bool(failure), readonly=True)
        if failure:
            self.assertEqual(mcp.pop("error")["code"], failure)
        self.equal_read(cli, mcp)
        return cli, mcp

    def test_the_layers_stay_apart_and_both_entries_agree_on_every_set_of_facts(self):
        seen_codes = set()
        # label, source, selectable, context complete, readiness diagnostics, failure of the prepared context
        cases = [
            ("every layer clear", WORK, True, True, set(), None),
            ("a required dependency is not done: the context is complete and the work is not selectable",
             self.NOT_DONE, False, True, {"dependency_not_completed"}, None),
            ("a required dependency that is not done and has no next action: the context is not complete either",
             WORK.replace("id: D\n  title: Required input\n  status: completed", "id: D\n  title: Required input\n  status: ready"),
             False, False, {"dependency_not_completed"}, "ContextIncomplete"),
            ("an active blocker: the context is complete and the work is not selectable",
             WORK.replace("next_action: Draft the analysis", "next_action: Draft the analysis\n  blocker: Waiting for the customer"),
             False, True, {"active_blocker"}, None),
            ("no acceptance: the work is selectable and the context is not complete",
             WORK.replace("  acceptance: [Deliver the reviewed analysis]\n", ""), True, False, set(), "ContextIncomplete"),
            ("a status nobody knows is neither selectable nor complete",
             WORK.replace("title: Prepare customer analysis\n  status: ready", "title: Prepare customer analysis\n  status: wibble"),
             False, False, {"unknown_status"}, "ContextIncomplete"),
            ("a dependency that does not exist is neither selectable nor complete",
             WORK.replace("depends_on: [D]", "depends_on: [D, GONE]"), False, False, {"missing_dependency"}, "ContextIncomplete"),
        ]
        for label, work, selectable, complete, codes, failure in cases:
            with self.subTest(label):
                self.facts(work)
                for cli, tool, args in [(["status", "--work", "W"], "awr_project_status", {"view": "action", "work": ["W"]}),
                                        (["ready"], "awr_work_ready", {}),
                                        (["work", "show", "W"], "awr_work_get", {"work": "W"})]:
                    self.equal_read(self.cli(cli), self.tool(tool, args, readonly=True))
                shown = self.cli(["work", "show", "W"])["work"]
                self.assertEqual(shown["ready"], selectable)
                self.assertEqual({d["code"] for d in shown["diagnostics"]}, codes)
                cli, mcp = self.prepared(failure)
                for prepared in (cli, mcp):
                    self.assertEqual(prepared["ready"], selectable)
                    self.assertEqual(prepared["context"]["completeness"]["complete"], complete)
                    self.assertEqual({d["code"] for d in prepared["diagnostics"]}, codes)
                    self.assertEqual(prepared["active_claims"], [], "reading claims nothing")
                seen_codes |= codes
        # Every code seen is explained in the diagnostics reference.
        reference = (Path(__file__).resolve().parents[3] / "docs/dev/reference/diagnostics.md").read_text()
        self.assertEqual({code for code in seen_codes if f"`{code}`" not in reference}, set())
        # A claim is its own answer: the work is no longer selectable and its context is still complete.
        self.facts(WORK)
        self.session()
        cli, mcp = self.prepared()
        for prepared in (cli, mcp):
            self.assertFalse(prepared["ready"])
            self.assertTrue(prepared["context"]["completeness"]["complete"])
            self.assertEqual({d["code"] for d in prepared["diagnostics"]}, {"active_claim"})
            self.assertEqual(len(prepared["active_claims"]), 1)
        self.assertTrue(mcp["read_only"] and not cli["read_only"])

    def test_a_query_authorizes_nothing_and_a_claim_is_judged_on_the_facts_of_that_moment(self):
        # Both entries say the work can be selected, and reading it claims nothing.
        self.assertTrue(self.cli(["work", "show", "W"])["work"]["ready"])
        self.assertTrue(self.tool("awr_work_get", {"work": "W"}, readonly=True)["work"]["ready"])
        self.assertEqual(self.cli(["work", "show", "W"])["work"]["active_claims"], [])
        # A required dependency becomes unfinished behind those answers: neither entry grants the claim.
        self.facts(self.NOT_DONE)
        revision = self.revision()
        _, cli_error = self.cli(["session", "start", "--work", "W", "--agent", "parity-executor", "--provider", "fixture",
                                 "--model", "no-model-call", "--claim", "--expected-revision", revision], code=1)
        mcp_error = self.tool("awr_session_start", {"work": "W", "conversation": "parity-query", "agent": "parity-executor",
                                                    "provider": "fixture", "model": "no-model-call",
                                                    "expected_revision": revision, "claim": True}, error=True)
        for error in (cli_error, mcp_error):
            self.assertEqual(error["code"], "DependencyBlocked")
            self.assertIn("dependency_not_completed", error["message"])
        self.assertEqual(self.cli(["work", "show", "W"])["work"]["active_claims"], [])
        # A lease that has ended authorizes nothing either, and the work shows as unclaimed.
        self.facts(WORK)
        sid = self.cli(["session", "start", "--work", "W", "--agent", "parity-executor", "--provider", "fixture",
                        "--model", "no-model-call", "--claim", "--ttl-ms", "1000", "--expected-revision", self.revision()])["session"]["id"]
        time.sleep(1.4)
        revision = self.revision()
        _, cli_error = self.cli(["work", "progress", "W", "--session", sid, "--reason", "Continue after the lease ended",
                                 "--next-action", "Continue", "--expected-revision", revision], code=1)
        mcp_error = self.tool("awr_work_transition", {"work": "W", "action": "progress", "session": sid,
                                                      "reason": "Continue after the lease ended", "next_action": "Continue",
                                                      "expected_revision": revision}, error=True)
        self.assertEqual(cli_error, mcp_error)
        self.assertEqual(cli_error["code"], "ClaimConflict")
        shown = self.cli(["work", "show", "W"])["work"]
        self.assertEqual(shown["active_claims"], [])
        self.assertTrue(shown["ready"])

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

    def test_check_path_matches_baseline_admission_acceptance_and_completion_and_detects_unmet_drift(self):
        criterion = "A saved draft survives a failed save"
        decision = '---\n{"id":"scope","status":"accepted","affected_keys":["*"]}\n---\n# Scope\n\n## Decision\nKeep the local draft available.\n'
        (self.root / "decisions").mkdir()
        # Use CRLF on every host to catch fingerprints computed from normalized text.
        decision_path = self.root / "decisions/scope.md"
        decision_path.write_bytes(decision.replace("\n", "\r\n").encode("utf-8"))
        manifest = (self.root / ".awr/project.toml").read_text()
        manifest = manifest.replace("adapter = \"yaml-ledger-v1\"", 'adapter = "yaml-ledger-v1"\n[sources.options.admission_policy]\nversion=1\nmode="governed_enforce"\nscope="all"')
        manifest += '\n[[sources]]\ndomain="decisions"\nrole="supporting"\npath="decisions"\nadapter="markdown-directory-v1"\n'
        (self.root / ".awr/project.toml").write_text(manifest)
        (self.root / ".awr/gates.toml").write_text('version=1\n[checks]\nmode="enforce"\nscope="all"\n')
        probe = 'from pathlib import Path\nraise SystemExit(0 if Path("draft.txt").read_text() == "retained" else 1)\n'
        (self.root / "check.py").write_text(probe)
        definition = {"version": 1, "purpose": "implementation", "tier": "required",
                      "intent": [{"role": "user_decision", "decision": "scope",
                                  "source_fingerprint": "sha256:" + hashlib.sha256(decision_path.read_bytes()).hexdigest()}],
                      "contributes": [{"goal": "G", "outcome": criterion, "claim": "Retains a local draft", "by": "parity-executor"}],
                      "granularity": {"judgment": "independent_result", "reason": "One observable save result", "by": "parity-executor"},
                      "responsibility": {"name": "draft-storage", "kind": "delivery"},
                      "deliverable": "A retained local draft", "evidence_method": "Run the same checks before and after the fix",
                      "failure_recovery": "Keep the draft after a failed save", "discrimination": {"method": "fail_before_pass_after"},
                      "examples": [{"id": kind, "criterion": criterion, "kind": kind, "given": "a pending draft",
                                    "when": "saving fails", "then": "the draft remains available",
                                    "check": {"run": [sys.executable, "check.py"], "files": ["check.py"]}}
                                   for kind in ("positive", "negative")]}
        ledger = {"goals": [{"id": "G", "title": "Retain drafts", "status": "active", "success_criteria": [criterion]}],
                  "work_items": [{"id": "W", "title": "Retain drafts", "status": "in_progress", "goal": "G",
                                  "paths": ["draft.txt"], "next_action": "Implement draft retention", "acceptance": [criterion],
                                  "definition": definition}]}
        (self.root / "work.yaml").write_text(json.dumps(ledger))
        (self.root / ".gitignore").write_text(".awr/\ndraft.txt\nsnapshot.db\n*-input.json\nreport.json\n")
        for args in (["init"], ["config", "user.email", "parity@example.invalid"], ["config", "user.name", "Parity fixture"],
                     ["add", "work.yaml", "rules.md", "goal.md", "decisions", "check.py", ".gitignore"],
                     ["commit", "-m", "Declare draft retention checks"]):
            subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True)
        sha = subprocess.check_output(["git", "-C", str(self.root), "rev-parse", "HEAD"], text=True).strip()
        self.cli(["source", "reindex"])
        sid = self.cli(["session", "start", "--work", "W", "--agent", "parity-executor", "--provider", "fixture",
                        "--model", "no-model-call", "--expected-revision", self.revision()])["session"]["id"]
        snapshot = self.snapshot()

        def path(mcp):
            trace = {}
            (self.root / "draft.txt").write_text("lost")

            def check(phase, error=False):
                args = {"work": "W", "session": sid, "phase": phase, "expected_revision": self.revision()}
                if mcp:
                    return self.tool("awr_work_check", args, error=error)
                command = ["work", "check", "W", "--session", sid, "--phase", phase, "--expected-revision", args["expected_revision"]]
                return self.cli(command, code=1)[1] if error else self.cli(command)

            refused = check("acceptance", error=True)
            self.assertEqual(refused["code"], "AdmissionRequired")
            trace["admission_refusal"] = refused
            trace["before_unmet"] = (self.tool("awr_work_get", {"work": "W", "source_sha": sha}, readonly=True)
                                     if mcp else self.cli(["work", "show", "W", "--source-sha", sha]))["checks"]["unmet"]
            baseline = check("baseline")
            self.assertTrue(baseline["met"])
            self.assertEqual([r["outcome"] for r in baseline["results"]], ["failed", "failed"])
            prepared = (self.tool("awr_work_prepare", {"work": "W", "session": sid, "budget": 9000}, readonly=True)
                        if mcp else self.cli(["work", "prepare", "W", "--session", sid, "--budget", 9000]))
            trace["obligations"] = prepared["check_obligations"]
            admission = {"work": "W", "session": sid, "expected_revision": self.revision(), "request_key": "approve-draft",
                         "decision": "approved", "contract_fingerprint": prepared["management"]["contract_fingerprint"],
                         "judgment": {"contribution": "Keeps the draft", "necessity": "Drafts are lost today",
                                      "alternatives": "Reuse the local buffer", "deliverable": "A retained draft",
                                      "exit": "All checks pass", "responsibility": "draft-storage"}}
            (self.root / "admit-input.json").write_text(json.dumps(admission))
            approved = self.tool("awr_work_admit", admission) if mcp else self.cli(["work", "admit", "--input", self.root / "admit-input.json"])
            trace["admission_state"] = approved["admission"]["state"]
            self.cli(["work", "claim", "W", "--session", sid, "--expected-revision", self.revision()])
            (self.root / "draft.txt").write_text("retained")
            acceptance = check("acceptance")
            self.assertTrue(acceptance["met"])
            self.assertEqual([r["outcome"] for r in acceptance["results"]], ["passed", "passed"])
            self.assertIsNotNone(acceptance["completion_input"])
            report = {"version": 1, "work_item": "W", "source_sha": sha, "command": "Inspect retained draft",
                      "scope": ["W"], "verified_at": 1,
                      "checks": [{"name": "draft", "passed": True, "details": "Draft retained", "criteria": [criterion]}]}
            (self.root / "report.json").write_text(json.dumps(report) + "\n")
            preflight = (self.tool("awr_completion_prepare", {"work": "W", "report": "report.json", "evidence_key": "review",
                                                              "source_sha": sha, "level": "locally_verified"}, readonly=True)
                         if mcp else self.cli(["work", "prepare-completion", "W", "--report", "report.json", "--evidence-key", "review",
                                              "--source-sha", sha, "--level", "locally_verified"]))
            trace["after_unmet"] = preflight["checks"]["unmet"]
            self.assertEqual(trace["after_unmet"], [])
            completion = acceptance["completion_input"]
            (self.root / "complete-input.json").write_text(json.dumps(completion))
            completed = (self.tool("awr_work_transition", {"work": "W", "session": sid, "action": "complete", "reason": "Retained draft verified",
                                                          "completion": completion, "expected_revision": self.revision()})
                         if mcp else self.cli(["work", "complete", "W", "--session", sid, "--reason", "Retained draft verified",
                                              "--input", self.root / "complete-input.json", "--expected-revision", self.revision()]))
            trace["completion"] = {key: completed[key] for key in ("ok", "code", "project_revision", "source_write_performed", "write_outcome", "error")}
            self.assertEqual(completed["event"]["event_type"], "work.completed")
            trace["completion_gate"] = completed["check_gate"]
            # Each execution allocates a fresh artifact identity. Validate its actual
            # bytes and source reference before comparing the remaining source text.
            locator = acceptance["evidence"]["locator"]
            report_bytes = (self.root / locator).read_bytes()
            self.assertEqual(hashlib.sha256(report_bytes).hexdigest(), acceptance["evidence"]["sha256"])
            source = (self.root / "work.yaml").read_text()
            self.assertIn(locator, source)
            trace["completed_source"] = source.replace(locator, "<generated-check-report>")
            with closing(sqlite3.connect(f"file:{self.root / '.awr/state.db'}?mode=ro", uri=True)) as db:
                events = [json.loads(row[0]) for row in db.execute("SELECT payload_json FROM events WHERE event_type='work.check' ORDER BY project_revision")]
            self.assertEqual(len(events), 4)
            trace["events"] = [{k: v for k, v in event.items() if k not in ("started_at", "finished_at")} for event in events]
            return trace

        cli = path(False)
        self.restore(snapshot)
        shutil.rmtree(self.root / ".awr/artifacts")
        mcp = path(True)
        self.assertEqual(cli, mcp)
        changed = deepcopy(mcp)
        changed["after_unmet"] = [{"example": "positive", "reason": "acceptance_missing"}]
        with self.assertRaises(AssertionError, msg="unmet drift must fail the comparison"):
            self.assertEqual(cli, changed)


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
