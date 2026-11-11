import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from run import CONTRACT, ROOT, source_fixture

sys.path.insert(0, str(ROOT / 'tests/platform'))
spec = importlib.util.spec_from_file_location('workflow_platform', ROOT / 'tests/platform/verify.py')
platform_gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(platform_gate)
sys.path.pop(0)


class CheckedCompletionTest(unittest.TestCase):
    def report(self):
        return dict(workflow_runs=12, checks={key: True for key in CONTRACT['required_checks']},
                    runtime_source_sha='a' * 40, binary_sha256=dict(awr='cli', mcp='mcp'),
                    missing_metrics=CONTRACT['missing_metrics'])

    def validate(self, data):
        platform_gate.validate_workflow(data, 'a' * 40, dict(awr='cli', mcp='mcp'))

    def test_checked_completion_is_in_both_strategy_runs_and_platform_contract(self):
        self.assertIn('checked_completion', CONTRACT['workloads'])
        expected = len(CONTRACT['workloads']) * len(CONTRACT['strategies'])
        self.assertEqual(expected, 12)
        platform_contract = json.loads((ROOT / 'tests/platform/contract.json').read_text())
        self.assertEqual(platform_contract['workflow_runs'], expected)
        self.validate(self.report())

    def test_added_workload_with_unchanged_verifier_count_fails_platform_gate(self):
        with patch.object(platform_gate, 'EXPECTED_WORKFLOW_RUNS', 10):
            with self.assertRaisesRegex(RuntimeError, 'workflow'):
                self.validate(self.report())

    def test_missing_runs_failed_checks_and_wrong_binary_bindings_fail_platform_gate(self):
        mutations = [('workflow_runs', 10), ('checks', {'acceptance': False}),
                     ('runtime_source_sha', 'b' * 40), ('binary_sha256', {'awr': 'other', 'mcp': 'mcp'}),
                     ('missing_metrics', {'model_cost': 0})]
        for key, value in mutations:
            with self.subTest(key=key):
                data = copy.deepcopy(self.report())
                data[key] = value
                with self.assertRaises(RuntimeError):
                    self.validate(data)

    def test_checked_fixture_declares_both_example_kinds_for_each_criterion(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'project'
            source_fixture(root, False, checked=True)
            ledger = json.loads((root / 'work.yaml').read_text())
            work = ledger['work_items'][0]
            examples = work['definition']['examples']
            for criterion in work['acceptance']:
                self.assertEqual({e['kind'] for e in examples if e['criterion'] == criterion},
                                 {'positive', 'negative'})
            self.assertTrue(all(e['check']['files'] for e in examples))
            self.assertIn("governed_enforce", (root / 'map.toml').read_text())

    def test_checked_fixture_oracle_accepts_artifact_and_rejects_missing_fields(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'project'
            source_fixture(root, False, checked=True)
            work = json.loads((root / 'work.yaml').read_text())['work_items'][0]
            artifact = root / 'W-artifact.json'
            for example in work['definition']['examples']:
                artifact.write_text(json.dumps(dict(work='W', result='Reviewed handoff', limitations='Synthetic fixture')))
                command = example['check']['run']
                self.assertEqual(subprocess.run(command, cwd=root, capture_output=True).returncode, 0)
                artifact.write_text(json.dumps(dict(work='W', result='', limitations='')))
                self.assertNotEqual(subprocess.run(command, cwd=root, capture_output=True).returncode, 0)

    def test_checked_fixture_binds_actual_crlf_decision_bytes(self):
        write_text = Path.write_text

        def windows_decision(path, data, *args, **kwargs):
            if path.name == 'scope.md':
                kwargs['newline'] = '\r\n'
            return write_text(path, data, *args, **kwargs)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'project'
            with patch.object(Path, 'write_text', windows_decision):
                source_fixture(root, False, checked=True)
            decision = (root / 'decisions/scope.md').read_bytes()
            self.assertIn(b'\r\n', decision)
            work = json.loads((root / 'work.yaml').read_text())['work_items'][0]
            pin = work['definition']['intent'][0]['source_fingerprint']
            self.assertEqual(pin, 'sha256:' + hashlib.sha256(decision).hexdigest())
            self.assertNotEqual(pin, 'sha256:' + hashlib.sha256(decision.replace(b'\r\n', b'\n')).hexdigest())


if __name__ == '__main__':
    unittest.main()
