#!/usr/bin/env python3
"""Lightweight contract tests for the evidence-currency runner itself."""

import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import uuid
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SPEC = importlib.util.spec_from_file_location("evidence_currency_verify", HERE / "verify.py")
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)
PLATFORM_SPEC = importlib.util.spec_from_file_location("platform_verify", ROOT / "tests" / "platform" / "verify.py")
PLATFORM = importlib.util.module_from_spec(PLATFORM_SPEC)
PLATFORM_SPEC.loader.exec_module(PLATFORM)


class VerifyContractTests(unittest.TestCase):
    def test_binary_snapshot_rejects_a_build_artifact_changed_during_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, destination = Path(temporary) / 'source', Path(temporary) / 'snapshot'
            source.write_bytes(b'original binary')
            original_copy = VERIFY.shutil.copy2

            def changed_during_copy(src, dst):
                original_copy(src, dst)
                source.write_bytes(b'replaced build artifact')

            with patch.object(VERIFY.shutil, 'copy2', changed_during_copy):
                with self.assertRaises(ValueError):
                    VERIFY.snapshot_binary(source, destination)
            source.write_bytes(b'final binary')
            sha = VERIFY.snapshot_binary(source, destination)
            source.write_bytes(b'a subsequent independent build')
            self.assertEqual(VERIFY.digest(destination), sha)

    def test_native_currency_archive_copies_complete_success_receipt_without_fixtures(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "poc"
            destination = Path(temporary) / "evidence" / "currency"
            source.mkdir()
            (source / "report.json").write_text('{"passed":true}\n', encoding="utf-8")
            (source / "000.stdout").write_text("top-level stdout\n", encoding="utf-8")
            (source / "001.stderr").write_text("top-level stderr\n", encoding="utf-8")
            (source / "cli_full_flow.failure.txt").write_text("scenario failure\n", encoding="utf-8")
            (source / "unrelated.json").write_text("must stay out\n", encoding="utf-8")
            fixture = source / "same project"
            (fixture / ".awr").mkdir(parents=True)
            (fixture / ".awr" / "state.db").write_bytes(b"private fixture state")

            archived = PLATFORM.archive_currency_artifacts(source, destination)
            names = sorted(path.name for path in destination.iterdir())
            self.assertEqual(names, ["000.stdout", "001.stderr", "cli_full_flow.failure.txt", "report.json"])
            receipt = Path(archived["receipt"])
            self.assertTrue(receipt.is_file())
            self.assertEqual(archived["archive_status"], "finalized")
            self.assertEqual(archived["receipt_sha256"], hashlib.sha256(receipt.read_bytes()).hexdigest())
            self.assertFalse((destination / "same project").exists())
            self.assertFalse((destination / "unrelated.json").exists())

    def test_native_currency_archive_preserves_ordinary_failure_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "poc"
            destination = Path(temporary) / "evidence" / "currency"
            source.mkdir()
            (source / "report.json").write_text('{"passed":false}\n', encoding="utf-8")
            (source / "002.stdout").write_text("failed scenario output\n", encoding="utf-8")
            (source / "cross_instance_transport.failure.txt").write_text(
                "failed scenario\n", encoding="utf-8")

            archived = PLATFORM.archive_currency_artifacts(source, destination)
            self.assertEqual(archived["archive_status"], "finalized")
            self.assertTrue((destination / "report.json").is_file())
            self.assertTrue((destination / "cross_instance_transport.failure.txt").is_file())
            self.assertEqual((destination / "report.json").read_text(encoding="utf-8"),
                             '{"passed":false}\n')

    def test_native_currency_archive_rejects_symlink_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "poc"
            destination = Path(temporary) / "evidence" / "currency"
            source.mkdir()
            (source / "report.json").write_text('{"passed":true}\n', encoding="utf-8")
            target = source / "real.stdout"
            target.write_text("real\n", encoding="utf-8")
            link = source / "000.stdout"
            try:
                link.symlink_to(target)
            except (NotImplementedError, OSError):
                self.skipTest("symlinks are unavailable on this host")
            with self.assertRaises(RuntimeError):
                PLATFORM.archive_currency_artifacts(source, destination)
            self.assertFalse(destination.exists())

    def test_native_currency_archive_rejects_artifact_changed_during_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / "poc"
            destination = Path(temporary) / "evidence" / "currency"
            source.mkdir()
            (source / "report.json").write_text('{"passed":true}\n', encoding="utf-8")
            artifact = source / "000.stdout"
            artifact.write_bytes(b"before")
            original_copy = PLATFORM.shutil.copyfile

            def changed(src, dst, **options):
                original_copy(src, dst, **options)
                if Path(src) == artifact:
                    artifact.write_bytes(b"after")

            with patch.object(PLATFORM.shutil, "copyfile", changed):
                with self.assertRaises(RuntimeError):
                    PLATFORM.archive_currency_artifacts(source, destination)
            self.assertFalse(destination.exists())

    def test_native_receipt_callback_loads_archived_json_instead_of_gate_log(self):
        with tempfile.TemporaryDirectory() as temporary:
            receipt = Path(temporary) / "report.json"
            receipt.write_text('{"passed":true,"source":"archive"}\n', encoding="utf-8")
            seen = []

            def validate(data):
                seen.append(data)
                return {"validated_source": data["source"]}

            result = PLATFORM.receipt(receipt, validate)("this is the gate log, not JSON")
            self.assertEqual(seen, [{"passed": True, "source": "archive"}])
            self.assertEqual(result["validated_source"], "archive")
            self.assertEqual(result["receipt_sha256"], hashlib.sha256(receipt.read_bytes()).hexdigest())

    def test_native_receipt_finalization_rejects_timeout_or_incomplete_cleanup(self):
        self.assertTrue(PLATFORM.receipt_finalized({
            "cleanup_complete": True, "log_finalized": True, "timed_out": False,
        }))
        for result in (
            {"cleanup_complete": False, "log_finalized": True, "timed_out": False},
            {"cleanup_complete": True, "log_finalized": False, "timed_out": False},
            {"cleanup_complete": True, "log_finalized": True, "timed_out": True},
        ):
            with self.subTest(result=result):
                self.assertFalse(PLATFORM.receipt_finalized(result))

    def test_native_currency_snapshot_rejects_hash_mismatch(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, destination = Path(temporary) / "awr", Path(temporary) / "snapshot"
            source.write_bytes(b"original binary")
            original_copy = PLATFORM.shutil.copy2

            def changed(src, dst, **options):
                original_copy(src, dst, **options)
                source.write_bytes(b"replaced binary")

            with patch.object(PLATFORM.shutil, "copy2", changed):
                with self.assertRaises(RuntimeError):
                    PLATFORM.snapshot_binary(source, destination)

    @classmethod
    def setUpClass(cls):
        cls.contract = json.loads((HERE / "contract.json").read_text(encoding="utf-8"))
        cls.scenarios = [item["id"] for item in cls.contract["scenarios"]]

    def valid_poc(self, binary):
        return {
            "version": 1,
            "passed": True,
            "scenarios": [{"id": item['id'], "passed": True,
                           "details": dict(item['required_observations'])}
                          for item in self.contract['scenarios']],
            "steps": [{"command": ["synthetic"]}],
            "inputs_unchanged": True,
            "input_hashes": {"fixture": "a" * 64},
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "binary_unchanged": True,
            "binary_identity": {
                "format": "ELF", "architecture": "x86_64", "host_architecture": "x86_64",
                "architecture_relation": "same",
            },
            "platform": {"system": "test", "machine": "test"},
            "source_dirty": True,
            "verification_kind": "synthetic_cli_process_poc",
        }

    def test_contract_inputs_are_present_and_scenario_count_is_stable(self):
        missing = [name for name in self.contract["input_paths"] if not (ROOT / name).is_file()]
        self.assertEqual(missing, [])
        self.assertEqual(len(self.scenarios), len(set(self.scenarios)))
        self.assertEqual(self.scenarios, [
            "cli_full_flow", "cross_instance_transport", "stale_revision_retry", "git_revision_strict",
        ])

    def test_output_guard_accepts_ignored_or_external_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            external = Path(temporary) / "new-receipt"
            self.assertEqual(VERIFY.output_path(external), external.resolve())
        local = ROOT / ".local" / f"evidence-currency-test-{uuid.uuid4().hex}"
        self.assertEqual(VERIFY.output_path(local), local.resolve())
        with self.assertRaises(ValueError):
            VERIFY.output_path(ROOT / "tests" / "evidence-currency" / f"unsafe-{uuid.uuid4().hex}")

    def test_poc_validator_rejects_missing_or_failed_scenarios(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "awr"
            binary.write_bytes(b"synthetic binary")
            valid = self.valid_poc(binary)
            self.assertEqual(
                VERIFY.validate_poc_report(valid, self.contract, self.scenarios, binary)["scenarios"],
                self.scenarios,
            )
            missing = copy.deepcopy(valid)
            missing["scenarios"] = missing["scenarios"][:-1]
            with self.assertRaises(ValueError):
                VERIFY.validate_poc_report(missing, self.contract, self.scenarios, binary)
            failed = copy.deepcopy(valid)
            failed["scenarios"][0]["passed"] = False
            with self.assertRaises(ValueError):
                VERIFY.validate_poc_report(failed, self.contract, self.scenarios, binary)
            missing_observation = copy.deepcopy(valid)
            missing_observation['scenarios'][1]['details'].pop('fresh_completion_event')
            with self.assertRaises(ValueError):
                VERIFY.validate_poc_report(missing_observation, self.contract, self.scenarios, binary)

    def test_compile_only_cannot_claim_native_execution(self):
        names = ["crash_recovery", "core", "store", "store_evidence", "source", "source_fingerprint",
                 "runtime", "context", "cli", "mcp", "poc"]
        statuses = {name: "passed" for name in names}
        rows = {"build": {"status": "passed", "built": True}}
        self.assertTrue(VERIFY.compile_gate_passed(rows))
        self.assertFalse(VERIFY.native_gate_passed("compile_only", statuses, names))
        self.assertTrue(VERIFY.native_gate_passed("native_test", statuses, names))
        rows["build"]["built"] = False
        self.assertFalse(VERIFY.compile_gate_passed(rows))

    def test_rust_counts_sum_each_reported_target_once(self):
        totals = VERIFY.rust_test_totals(
            "test result: ok. 2 passed; 0 failed; 1 ignored;\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored;\n"
        )
        self.assertEqual(totals, [(2, 0, 1), (1, 0, 0)])
        self.assertEqual(sum(row[0] for row in totals), 3)


if __name__ == "__main__":
    unittest.main(verbosity=2)
