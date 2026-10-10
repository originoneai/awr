"""Actual subprocess regressions for native verification receipt finalization."""
import hashlib
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from process_runner import run_command


class ProcessRunnerTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='awr-platform-process-')
        self.root = Path(self.directory.name).resolve()
        self.log = self.root / 'command.log'

    def tearDown(self):
        self.directory.cleanup()

    def run_python(self, code, timeout=5):
        return run_command([sys.executable, '-u', '-c', code], cwd=self.root,
                           log=self.log, timeout_seconds=timeout, cleanup_seconds=5)

    def test_success_preserves_complete_stdout_and_stderr(self):
        result = self.run_python("import sys; print('out'); print('err', file=sys.stderr)")
        self.assertTrue(result['passed'], result)
        self.assertEqual(result['exit_code'], 0)
        self.assertTrue(result['log_finalized'])
        self.assertTrue(result['cleanup_complete'])
        self.assertEqual(self.log.read_bytes().splitlines(), [b'out', b'err'])

    def test_nonzero_exit_keeps_output_and_never_passes(self):
        result = self.run_python("import sys; print('failed'); sys.exit(9)")
        self.assertFalse(result['passed'])
        self.assertEqual(result['exit_code'], 9)
        self.assertIn(b'failed', self.log.read_bytes())
        self.assertTrue(result['cleanup_complete'])

    def test_missing_command_never_passes_or_leaves_an_open_log(self):
        result = run_command([str(self.root / 'no-such-executable')], cwd=self.root,
                             log=self.log, timeout_seconds=5, cleanup_seconds=5)
        self.assertFalse(result['passed'])
        self.assertTrue(result['log_finalized'])
        if os.name != 'nt':
            self.assertEqual(self.log.stat().st_size, 0)

    def test_timeout_reaps_the_launcher_and_retains_partial_output(self):
        result = self.run_python("import time; print('started', flush=True); time.sleep(20)", timeout=0.5)
        self.assertFalse(result['passed'])
        self.assertTrue(result['timed_out'])
        self.assertIsNotNone(result['exit_code'])
        self.assertTrue(result['cleanup_complete'], result)
        self.assertTrue(result['log_finalized'])
        self.assertIn(b'started', self.log.read_bytes())

    def family_code(self, parent_exits):
        marker = self.root / 'late-write'
        ready = self.root / 'ready'
        leaf = ("import pathlib,time; print('leaf-ready',flush=True); pathlib.Path(" + repr(str(ready))
                + ").write_text('ready'); time.sleep(2); pathlib.Path("
                + repr(str(marker)) + ").write_text('escaped'); print('late-output',flush=True)")
        middle = "import subprocess,sys,time; subprocess.Popen([sys.executable,'-u','-c'," + repr(leaf) + "]); time.sleep(20)"
        parent = ("import pathlib,subprocess,sys,time; ready=pathlib.Path(" + repr(str(ready))
                  + "); subprocess.Popen([sys.executable,'-u','-c'," + repr(middle) + "])"
                  + "\nfor _ in range(500):\n if ready.exists(): break\n time.sleep(0.01)\nelse: raise RuntimeError('Child did not start')\n")
        if not parent_exits:
            parent += "time.sleep(20)\n"
        return parent, marker

    def assert_family_stopped_and_log_stable(self, result, marker):
        self.assertTrue(result['cleanup_complete'], result)
        self.assertTrue(result['log_finalized'])
        before = hashlib.sha256(self.log.read_bytes()).hexdigest()
        self.assertIn(b'leaf-ready', self.log.read_bytes())
        time.sleep(2.2)
        self.assertFalse(marker.exists(), 'Grandchild survived the runner cleanup')
        self.assertEqual(hashlib.sha256(self.log.read_bytes()).hexdigest(), before)
        self.assertNotIn(b'late-output', self.log.read_bytes())

    def test_timeout_terminates_grandchildren_before_final_log_hash(self):
        code, marker = self.family_code(parent_exits=False)
        result = self.run_python(code, timeout=1)
        self.assertFalse(result['passed'])
        self.assertTrue(result['timed_out'])
        self.assert_family_stopped_and_log_stable(result, marker)

    def test_normal_exit_closes_orphan_output_before_final_log_hash(self):
        code, marker = self.family_code(parent_exits=True)
        result = self.run_python(code)
        self.assertTrue(result['passed'], result)
        self.assertFalse(result['timed_out'])
        self.assert_family_stopped_and_log_stable(result, marker)

    def test_cleanup_failure_is_explicit_even_after_zero_exit(self):
        with patch('process_runner._terminate_family', side_effect=OSError('synthetic cleanup failure')):
            result = self.run_python("print('finished')")
        self.assertEqual(result['exit_code'], 0)
        self.assertFalse(result['cleanup_complete'])
        self.assertFalse(result['passed'])
        self.assertIn('synthetic cleanup failure', result['error'])

    def test_budgets_reject_zero_negative_infinite_and_boolean_values(self):
        for budget in (0, -1, float('inf'), float('nan'), True):
            with self.subTest(budget=budget), self.assertRaises(ValueError):
                self.run_python("print('unused')", timeout=budget)
        self.assertFalse(self.log.exists())

    def test_cleanup_budget_must_also_be_finite_and_positive(self):
        for budget in (0, -1, float('inf'), float('nan'), True):
            with self.subTest(budget=budget), self.assertRaises(ValueError):
                run_command([sys.executable, '-c', "print('unused')"], cwd=self.root,
                            log=self.log, timeout_seconds=5, cleanup_seconds=budget)
        self.assertFalse(self.log.exists())

    def test_existing_evidence_is_never_overwritten(self):
        self.log.write_bytes(b'original evidence')
        with self.assertRaises(FileExistsError):
            self.run_python("print('replacement')")
        self.assertEqual(self.log.read_bytes(), b'original evidence')

    def test_contract_keeps_routes_and_gives_only_intel_workspace_extra_time(self):
        contract = json.loads((Path(__file__).parent / 'contract.json').read_text())
        self.assertEqual(contract['command_timeout_seconds'], 1800)
        self.assertEqual(contract['cleanup_timeout_seconds'], 10)
        for selected in contract['platforms']:
            expected = {'workspace': 3600} if selected['id'] == 'macos-x64' else {}
            self.assertEqual(selected.get('gate_timeout_seconds', {}), expected)
        self.assertIn('runner_tests', contract['required_gates'])
        self.assertIn('workspace', contract['required_gates'])
        self.assertIn('crash_recovery', contract['required_gates'])
        self.assertEqual(set(contract['ignored_tests']), set(contract['ignored_routes']))

    def test_unfinished_cleanup_stops_later_gates_and_has_no_final_log_digest(self):
        import verify
        contract = json.loads((Path(__file__).parent / 'contract.json').read_text())
        contract_path = self.root / 'contract.json'
        contract_path.write_text(json.dumps(contract))
        source = 'a' * 40
        metadata = {'packages': [{'name': name, 'targets': []} for name in contract['workspace_members']],
                    'target_directory': str(self.root / 'target')}
        observations = {('git', 'status', '--porcelain'): '', ('git', 'rev-parse', 'HEAD'): source,
                        ('git', 'rev-parse', 'HEAD^{tree}'): 'b' * 40, ('git', 'ls-files', '-z'): '',
                        ('rustc', '-vV'): 'release: 1.93.1\nhost: aarch64-apple-darwin',
                        ('cargo', '--version'): 'synthetic cargo observation',
                        ('cargo', 'metadata', '--locked', '--no-deps', '--format-version', '1'): json.dumps(metadata)}

        def unfinished(command, **options):
            Path(options['log']).write_bytes(b'unfinished synthetic output')
            return dict(exit_code=0, passed=False, timed_out=False,
                        cleanup_complete=False, log_finalized=False)

        output = self.root / '.local' / 'verify-cleanup-failure'
        with patch.object(verify, 'ROOT', self.root), patch.object(verify, 'CONTRACT', contract_path), \
                patch.object(verify, 'read_command', side_effect=lambda *args: observations[args]), \
                patch.object(verify.platform, 'system', return_value='Darwin'), \
                patch.object(verify.platform, 'machine', return_value='arm64'), \
                patch.object(verify, 'run_command', side_effect=unfinished) as execute, \
                patch.dict(os.environ, {'GITHUB_SHA': '', 'CARGO_BUILD_TARGET': ''}), \
                patch.object(sys, 'argv', ['verify.py', '--platform', 'macos-arm64', '--output', str(output)]), \
                contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(verify.main(), 1)
        execute.assert_called_once()
        report = json.loads((output / 'evidence' / 'report.json').read_text())
        first = next(gate for gate in report['gates'] if gate['id'] == 'runner_tests')
        self.assertFalse(first['passed'])
        self.assertNotIn('log_sha256', first)
        later = [gate for gate in report['gates'] if 'command' in gate and gate['id'] != 'runner_tests']
        self.assertTrue(later)
        self.assertTrue(all(gate['not_run'] and 'process_cleanup' in gate['unmet_dependencies'] for gate in later))
        self.assertFalse(report['passed'])


if __name__ == '__main__':
    unittest.main(verbosity=2)
